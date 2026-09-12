# Overview

This document states what Keel is, where the boundary of its kernel lies, how the codebase is
layered, how the repository is organised until the SDK reaches 1.0, and how the three versioning
planes relate. The other documents in `docs/design/` go into each subsystem; this one gives the
frame they all fit into.

## What Keel is

Keel is the framework on which every product of the line is built: KeelDNS, KeelPanel, and
later KeelBill, KeelVM and KeelIP. The kernel knows nothing about servers, zones or tariffs. It
knows how to execute a function, who is calling it, how to describe the function's screen, and
how several products live together.

A product has a brand, used in the interface and in documents, and an id, used by everything
machine-readable: the binary, the unix user, the systemd units, the directories, the database.

| Brand     | Product id | Binary, unit, user | Directories                                   | Database     |
|-----------|------------|--------------------|-----------------------------------------------|--------------|
| KeelDNS   | `dns`      | `keel-dns`         | `/opt/keel/dns`, `/etc/keel/dns`, `/var/lib/keel/dns`, `/run/keel/dns` | `keel_dns`   |
| KeelPanel | `panel`    | `keel-panel`       | `/opt/keel/panel`, ...                        | `keel_panel` |
| KeelBill  | `bill`     | `keel-bill`        | `/opt/keel/bill`, ...                         | `keel_bill`  |

The hosting product's id is `panel`, not `host`: `host` is the id of the module that `keel-host`
registers (functions `host.*`, the `host` section of the configuration). Module ids are
independent of product brands: the module crates of KeelDNS are `dns-zones`, `dns-records` and
`dns-templates`, those of KeelPanel are `hosting-*`. Files a product owns in shared include
directories start with `keel-<product>-`.

| Aspect   | Choice                                                |
|----------|-------------------------------------------------------|
| Backend  | Rust, axum, sea-orm, sqlmig                            |
| Storage  | Postgres on the host, one database per product         |
| Frontend | Angular 19, `@globalart/keel` on top of phoenix-ui     |
| Delivery | one static binary per product                          |

## What the kernel is and what it is not

An earlier revision of this design was a server control panel with an executor and a
configuration engine inside the kernel. That was a mistake of scale: the kernel of an ecosystem
must carry equally well a product that never touches the operating system (billing) and a product
that lives in `/etc` (hosting). The boundary is therefore drawn by a test rather than by a list of
features.

### Kernel membership test

A subsystem belongs to the kernel only if both conditions hold:

- without it the product does not start and does not answer on its local socket;
- it cannot be replaced by changing the product's module composition without editing its
  neighbours.

By this test the kernel keeps the function dispatcher, descriptors, principal and account, storage
with migrations, the outbox, the message catalogue, the manifest and the transports. Everything
else (authentication, tasks, notifications, license, updates, plugins, peer products) is a
system module written against the same public SDK as product modules. System modules are
described in [05-modules.md](05-modules.md); the dispatcher and descriptors in
[02-functions.md](02-functions.md); accounts and principals in [03-access.md](03-access.md);
storage in [04-storage.md](04-storage.md).

### What a product is

A product is a bin crate: a manifest, an explicit list of modules, a brand, editions. It builds
into one static binary (musl, rustls, frontend embedded) with the subcommands `serve`, `ctl`,
`privileged`, `migrate`, `backup`, `restore`, `doctor` and `support-bundle`. Module composition is
an explicit builder in `main.rs`, so the composition is visible and checked by the compiler:

```rust
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

The first products are `demo` (dogfood, no host access), KeelDNS (`dns`) and KeelPanel
(`panel`); KeelBill (`bill`), KeelVM (`vm`) and KeelIP (`ip`) come later on the same kernel. Delivery, directories and units per product
are in [09-operations.md](09-operations.md); the milestone order is in [../plan.md](../plan.md).

### What a shared library is

Server operations are a shared library, `keel-host`, linked only by products that manage a host.
The kernel does not depend on it, and CI verifies that over the cargo dependency graph. A billing
or IP-management product gets a binary without a host agent, which is what proves the kernel is
clean. The root boundary and the agent are described in [06-host.md](06-host.md). The other
shared libraries, `keel-host-agent`, `keel-storage` and `keel-client`, follow the same rule: the
kernel never imports them.

### How products relate to each other

Between products there is one call protocol, a shared client identity, pairing of instances,
events delivered through the outbox, and services for the future billing product. All of this is
built above the kernel and is described in [07-ecosystem.md](07-ecosystem.md).

### Out of scope

Not part of this project: container delivery, Windows, a plugin marketplace, the license server
itself, and billing as a product. The license server and billing will be built on this same
kernel later.

### Naming rule

Keel uses its own vocabulary for everything it names. `kind`, `role`, `control`, `level`, column
types, check presets and the rest of the descriptor vocabulary are named in Keel's own words; no
names are borrowed. The vocabulary is closed by enums in `keel-types` (see
[02-functions.md](02-functions.md)). Before the first release the identifiers in code, commits and
repository documentation are audited for accidental name coincidences; the audit is part of the M7
completion criteria in [../plan.md](../plan.md).

## Layers

Dependency arrows point only downwards:

```
product → libraries and modules → SDK → kernel → types
```

The kernel imports neither `keel-system` nor `keel-host`.

| Layer | Crates / packages | Explicitly excluded |
|-------|-------------------|---------------------|
| L1 Kernel | `keel-types`, `keel-kernel` | passwords and sessions, tasks, notifications, license, updates, plugins, any OS operation, minijinja, gRPC, Kafka, knowledge of a "node", a "service" or a "tariff" |
| L2 SDK | `keel-sdk` (plus the hidden `keel_sdk::system` for system modules), `keel-testkit` | implementations: the SDK knows nothing about axum, the Postgres pool or cookies |
| L3 System modules | `keel-system`: one crate, one cargo feature per module; depends only on `keel-sdk` | OS operations (`update` talks to the root unit, not to `keel-host`) |
| L4 Libraries | `keel-host`, `keel-host-agent`, `keel-storage`, `keel-client` | knowledge of nginx or bind as such; templates are brought by the product |
| L5 Products | `products/demo`, `products/dns`, `products/panel` | own transports, own auth, own error format, own updates; users outside `keel_principal` |
| L6 Frontend | `@globalart/keel` (entry points), `@globalart/keel-shell-app` | the design system (external phoenix-ui); runtime loading of UI plugins |
| L7 Tools | `keel-dev`, `keel-conformance`, `hack/` | not part of the product binary |

The facade crate `keel` sits between products and the layers below it: it re-exports the SDK and
the kernel's product builder (`keel::Product`, `keel::function`, `keel::embed_frontend!`).
Products depend only on the facade `keel` and on `@globalart/keel`.

The dependency rule at module level: a module depends on `keel-sdk` and, optionally, on the `api`
of modules listed in its own `requires`; importing anything else is an error reported by
`keel-dev check`. `keel-system` depends only on `keel-sdk`, which cargo verifies, and CI runs the
matrix "each system module alone, and the product without each module in turn" plus the test
"a product built from the kernel alone starts and answers on the socket". Hooks needed only by
system modules (`Authenticator`, `TransportRegistry`, `StorageSnapshot`, `OutboxSubscriber`) live
in `keel_sdk::system`, which is `#[doc(hidden)]` and exempt from semver until 1.0. Details are in
[05-modules.md](05-modules.md).

The frontend layer mirrors the same split: `@globalart/keel/meta` carries the wire types with no
Angular dependency, `/client`, `/forms`, `/renderer` and `/shell` build on it, and phoenix-ui
receives only atomic components while all metadata logic stays in `@globalart/keel`
([08-frontend.md](08-frontend.md)).

## Repository layout

Until the SDK reaches 1.0, everything (kernel, SDK, system modules, libraries, frontend, tools
and products) lives in one monorepo. The reason is the working conditions: one engineer, a
contract that is still moving, and most changes touching the Rust kernel, the host library, the
TypeScript packages and a product at once. In a monorepo that is one commit and one CI run.

Products depend only on the facade `keel` and on `@globalart/keel`, so extracting a product into
its own repository later is cheap (a git dependency on a published tag), while merging separate
repositories back would be expensive. The boundaries "kernel does not depend on host" and
"modules see only the SDK" are held by the cargo dependency graph and `keel-dev check`, not by
repository boundaries. No private cargo registry is needed before products are extracted.
phoenix-ui and sqlmig stay in their own repositories and are consumed as dependencies.

The cargo workspace has `members = ["crates/*", "products/*"]`; the frontend is a pnpm workspace.
The source leaves open whether product module crates sit under `crates/` (`crates/dns-zones`) or
nested inside the product directory (`products/dns/crates/dns-zones`); in the latter case the
workspace glob must be extended to `products/*/crates/*`. The table below lists them with the
product they belong to, not as a fixed path.

| Path | Contents |
|------|----------|
| `crates/types` | `keel-types`: descriptors, envelope, Principal/Account/Level, ListQuery/Page, events, `expr`, `check`; no I/O |
| `crates/sdk` | `keel-sdk`: `ModuleDef`, `Registrar`, `Call`, contributions, guards, `ListSource`, prelude |
| `crates/kernel` | `keel-kernel`: boot, config, module loader, dispatcher, metadata, access, storage, outbox, i18n, manifest, HTTP and `ctl` transports; kernel migrations |
| `crates/system` | `keel-system`: system modules, one cargo feature per module |
| `crates/host` | `keel-host`: `HostExecutor` client and the `host` module |
| `crates/host-agent` | `keel-host-agent`: the agent side, run as the `privileged` subcommand |
| `crates/storage` | `keel-storage`: local and S3 backup targets, encryption |
| `crates/client` | `keel-client`: Rust client for a peer product |
| `crates/testkit` | `keel-testkit` |
| `crates/conformance` | `keel-conformance` (bin) |
| `crates/dev` | `keel-dev` (bin) |
| `crates/keel` | `keel`: the facade: product builder, `embed_frontend!`, re-exports |
| `products/demo` | dogfood product without host access: manifest and the `note` module |
| `products/dns` | product crate; `brand/`, `agent.yaml`, `i18n/`, release keys; owns the module crates `dns-zones`, `dns-records`, `dns-templates` |
| `products/panel` | product crate; custom views; owns the module crates `hosting-users`, `hosting-web`, `hosting-mail`, `hosting-db`, `hosting-ftp`, `hosting-ssl`, `hosting-backup`, `hosting-services` |
| `frontend/` | pnpm workspace, Angular 19: `libs/keel` (entry points `meta`, `client`, `forms`, `renderer`, `shell`, `testing`), `apps/shell-app`, one app per product that has custom views, Storybook |
| `contracts/` | `protocol.md`, `fixtures/*.json`, generated `openapi.json` committed for diffs |
| `plugins/` | Python plugin SDK and example plugins |
| `deploy/` | `install.sh`, systemd units, sample `config.yaml` and `agent.yaml` |
| `hack/` | `prove-binary.sh`, `vm-test.sh`, `release.py`, `check-deps.sh` |
| `docs/` | operator, module developer, plugin protocol, descriptor and migration documentation |
| `.github/workflows/` | `ci.yml`, `release.yml`, `publish-npm.yml` |

Crates are created lazily: about thirteen in total, five to six at M0, which needs `keel-types`,
`keel-sdk`, `keel-kernel`, `keel-testkit` and `products/demo`. A product without custom screens
does not keep an Angular workspace at all; it embeds the prebuilt `@globalart/keel-shell-app`.

The monorepo carries one known risk: the temptation to bypass the SDK "just for a minute". The
countermeasures are `keel-dev check` linting imports, products depending only on the facade,
`keel-conformance` running against the built binary, and an "extraction rehearsal" (a dry run of
extracting a product from the facade) from M4 onwards. At M7 `dns` and `panel` are
extracted into their own repositories by published tag and build against the published
`keel-v1.0.0`. The alternatives considered for this decision are recorded in
[11-decisions.md](11-decisions.md).

## Versioning

There are three versioning planes, each with its own rule.

| Plane | What is versioned | Rule |
|-------|-------------------|------|
| Compile-time | `keel-sdk` | semver; the kernel and the SDK are released under one version; `cargo-semver-checks` runs on the facade from the first commit; the public `keel-sdk` is frozen at 1.0, `keel_sdk::system` is semver-exempt until then |
| Wire | `PROTOCOL` | reported in `GET /api/v1/instance` and in the plugin manifest; versions N and N-1 are supported |
| Data | descriptor `"keel": 1`, `/api/v1` | a breaking change ships as `/api/v2` alongside `/api/v1`; the major version of `@globalart/keel` equals the descriptor version |

Tags follow `keel-vX.Y.Z` for the kernel, SDK and npm package and `<product>-vX.Y.Z` for
products (`dns-v1.0.0`, `panel-v1.0.0`). A product's version is independent of the platform
version. The wire contract itself (envelope, descriptor, list contract, golden fixtures) is
specified in [10-contracts.md](10-contracts.md).
