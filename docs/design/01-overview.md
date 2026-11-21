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
machine-readable: the grouping of modules, the menu groups, the license features, the prefix
`keel-<product>-` of the files it owns in shared include directories, the prefix of the
operating-system users it creates, and the `--product` flag of `install.sh` and
`keel product enable`. Every product on a host runs on one installation of Keel: one binary
`keel`, one process, one database `keel`, one privileged agent, one set of directories and units,
one update. Installing a product means enabling its bundle on that installation
([05-modules.md](05-modules.md), [09-operations.md](09-operations.md)).

| Brand     | Product id | Module crates                               | Files in shared include directories |
|-----------|------------|---------------------------------------------|-------------------------------------|
| KeelDNS   | `dns`      | `dns-zones`, `dns-records`, `dns-templates` | `keel-dns-*`                        |
| KeelPanel | `panel`    | `hosting-*`, `dns-local`, `dns-peer`        | `keel-panel-*`                      |
| KeelBill  | `bill`     | later                                       | none: no host access                |

What belongs to the installation is the same on every host, whatever products are enabled:

| Item                | Name                                                                  |
|---------------------|-----------------------------------------------------------------------|
| binary and CLI      | `keel`                                                                |
| unix user and group | `keel`                                                                |
| units               | `keel.service`, `keel-privileged.service`, `keel-update.path`         |
| directories         | `/opt/keel`, `/etc/keel`, `/var/lib/keel`, `/run/keel`                |
| database            | `keel`, one schema `keel`; tables keep their module prefix            |
| session cookie      | `__Host-keel_session`                                                 |
| listen port         | one per installation; the default is open ([11-decisions.md](11-decisions.md)) |

The hosting product's id is `panel`, not `host`: `host` is the id of the module that `keel-host`
registers (functions `host.*`, the `host` section of the configuration). Module ids are
independent of product brands: the module crates of KeelDNS are `dns-zones`, `dns-records` and
`dns-templates`, those of KeelPanel are `hosting-*`. Files a product owns in shared include
directories start with `keel-<product>-`.

| Aspect   | Choice                                                                                             |
|----------|----------------------------------------------------------------------------------------------------|
| Backend  | Rust, axum, sea-orm, sqlmig                                                                        |
| Storage  | Postgres on the host, one database `keel` per installation                                        |
| Frontend | Angular 19, `@globalart/keel` on top of phoenix-ui                                                 |
| Delivery | one static binary `keel` carrying every first-party product; products are enabled per installation |

## What the kernel is and what it is not

An earlier revision of this design was a server control panel with an executor and a
configuration engine inside the kernel. That was a mistake of scale: the kernel of an ecosystem
must carry equally well a product that never touches the operating system (billing) and a product
that lives in `/etc` (hosting). The boundary is therefore drawn by a test rather than by a list of
features.

### Kernel membership test

A subsystem belongs to the kernel only if both conditions hold:

- without it the installation does not start and does not answer on its local socket;
- it cannot be replaced by changing the module composition without editing its neighbours.

By this test the kernel keeps the function dispatcher, descriptors, principal and account, storage
with migrations, the outbox, the message catalogue, the manifest and the transports. Everything
else (authentication, tasks, notifications, license, updates, plugins, peer products) is a
system module written against the same public SDK as product modules. System modules are
described in [05-modules.md](05-modules.md); the dispatcher and descriptors in
[02-functions.md](02-functions.md); accounts and principals in [03-access.md](03-access.md);
storage in [04-storage.md](04-storage.md).

### What a product is

A product is a bundle: a lib crate exporting `pub static MANIFEST: ProductManifest` — the
product's id, name and brand, the definitions of the modules it enables, the bridges through
which it reaches other products, its menu groups, its level labels, its editions with their
feature sets, the account kinds it creates and its own dictionaries
([05-modules.md](05-modules.md)) — together with the module crates it names. A bundle builds
nothing on its own. Every first-party bundle and every module it names are compiled into the one
binary `keel`; nothing is loaded dynamically. The composition is an explicit builder,
`keel::Runtime`, in `bin/keel/src/main.rs`, so it is visible and checked by the compiler; a
bundle brings the modules it names, and `.module()` lists the platform modules no bundle names:

```rust
// bin/keel/src/main.rs
fn main() -> keel::ExitCode {
    keel::Runtime::new()
        .module(&keel_system::auth::MODULE).module(&keel_system::settings::MODULE)
        .module(&keel_system::audit::MODULE).module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE).module(&keel_system::notify::MODULE)
        .module(&keel_system::license::MODULE).module(&keel_system::update::MODULE)
        .module(&keel_system::backup::MODULE).module(&keel_system::webhooks::MODULE)
        .module(&keel_system::peers::MODULE).module(&keel_system::svc::MODULE)
        .module(&keel_system::stats::MODULE)
        .product(&keel_dns::MANIFEST).product(&keel_panel::MANIFEST)
        .frontend(keel::embed_frontend!("../../frontend/dist/shell"))
        .run()
}
```

The kernel's own module `keel` is composed first by the loader and is never listed. The `demo`
bundle and its `note` module join the composition behind the cargo feature `demo` of `bin/keel`,
which is on by default so that the workspace, CI and `hack/prove-binary.sh` carry it, and off in
release builds through `--no-default-features`. Since M1 the composition lists the three system
modules `auth`, `settings` and `audit` and the `demo` bundle, and since M2 the shell through
`embed_frontend!`.

On a host the binary is one installation with the subcommands `serve`, `ctl`, `product`,
`privileged`, `migrate`, `backup`, `restore`, `doctor` and `support-bundle`. Which bundles are
enabled is recorded in the installation's database: `install.sh --product panel` enables one on a
clean host, `keel product enable panel` on an existing installation, and `keel product disable`
stops loading the product's modules and leaves their tables in place as orphaned. A module that
no bundle names belongs to the platform and is always loaded; a module named by a bundle is
loaded only while that bundle is enabled, and once when two enabled bundles name it. The enabled
set is gated by the license lease, which lists products and their editions.

The first bundles are `demo` (dogfood, no host access, compiled in only in development and CI),
KeelDNS (`dns`) and KeelPanel (`panel`); KeelBill (`bill`), KeelVM (`vm`) and KeelIP (`ip`) come
later on the same binary. Delivery, directories and units of the installation are in
[09-operations.md](09-operations.md); the milestone order is in [../plan.md](../plan.md).

### What a shared library is

Server operations are a shared library, `keel-host`, linked by the `keel` binary because the
first-party bundles that manage a host name its `host` module; a third-party composition built
from the facade may omit it. The kernel does not depend on it, and CI verifies that over the cargo
dependency graph. On an installation where no enabled bundle names `host` the module is not
loaded, the kernel never connects to the agent and the privileged unit serves the `update` policy
alone. What proves the kernel clean is the cargo graph (`hack/check-deps.sh`) and the CI
composition of the kernel without `keel-host`, not the absence of the agent from a shipped binary.
The root boundary and the agent are described in [06-host.md](06-host.md). The other shared
libraries, `keel-host-agent`, `keel-storage` and `keel-client`, follow the same rule: the kernel
never imports them. `keel-client`, the Rust client of the wire contract, depends on `keel-types`
alone among the workspace crates and the SDK does not import it either; `keel-conformance` is a
client on it, and a product module crate may depend on it as on `keel-host`, as `dns-peer` does to
reach a neighbouring installation ([05-modules.md](05-modules.md)); `hack/check-deps.sh` holds the
rules.

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
| L2 SDK | `keel-sdk` (plus the hidden `keel_sdk::system` for system modules), `keel-testkit`, which a module's tests see as part of the SDK although it links `keel-kernel` to boot the module in process | implementations: the SDK knows nothing about axum, the Postgres pool or cookies |
| L3 System modules | `keel-system`: one crate, one cargo feature per module; depends only on `keel-sdk` | OS operations (`update` talks to the root unit, not to `keel-host`) |
| L4 Libraries | `keel-host`, `keel-host-agent`, `keel-storage`, `keel-client` | knowledge of nginx or bind as such; templates are brought by the product |
| L5 Products | `products/demo`, `products/dns`, `products/panel` (bundles and their module crates); `bin/keel` (the composition) | own transports, own auth, own error format, own updates; users outside `keel_principal`; a process, a database, a session or an account tree of their own |
| L6 Frontend | `@globalart/keel` (entry points), `frontend/apps/shell` (the one shell application) | the design system (external phoenix-ui); runtime loading of UI plugins |
| L7 Tools | `keel-dev` (`types`, `descriptors dump \| diff \| check`, `views share`, `openapi`, `i18n extract \| missing \| pull`), `keel-conformance`, `keel-release` (`keygen`, `sign`, `verify`), `hack/` | not part of the `keel` binary |

The facade crate `keel` sits between products and the layers below it: it re-exports the SDK
(`keel::sdk`, `keel::prelude`, `keel::ModuleDef`, `keel::ProductManifest`; the function builder
is `keel::sdk::function`, also brought in by the prelude) and
the types (`keel::types`), and it owns the runtime builder `keel::Runtime` with what it returns
(`Running`, `Config`, `Change`, `ProductState`) and, from M2, `keel::embed_frontend!`. Bundles
and module crates depend only on the facade `keel` and on `@globalart/keel`; `bin/keel` is the
only crate that depends on them.

The dependency rule at module level: a module depends on `keel-sdk` and, optionally, on the `api`
of modules listed in its own `requires`; importing anything else is an error reported by
`keel-dev check`. `keel-system` depends only on `keel-sdk`, which cargo verifies, and CI runs the
matrix "each system module alone, and the composition without each module in turn" plus the test
"a composition of the kernel alone starts and answers on the socket". Hooks needed only by
system modules (the `Authenticator` chain, `PrincipalSource`, `SecretCipher`,
`SecondFactorSource`, `PreferenceSource`, `AuditSink`, `ModuleSource`, `PluginLauncher`; later
`TransportRegistry`,
`StorageSnapshot`, `OutboxSubscriber`) live in `keel_sdk::system`, which is `#[doc(hidden)]` and
exempt from semver until 1.0. Details are in [05-modules.md](05-modules.md).

The frontend layer mirrors the same split: `@globalart/keel/meta` carries the wire types with no
Angular dependency, `/client`, `/forms`, `/renderer` and `/shell` build on it, and phoenix-ui
receives only atomic components while all metadata logic stays in `@globalart/keel`
([08-frontend.md](08-frontend.md)).

## Repository layout

Until the SDK reaches 1.0, everything (kernel, SDK, system modules, libraries, frontend, tools
and products) lives in one monorepo. The reason is the working conditions: one engineer, a
contract that is still moving, and most changes touching the Rust kernel, the host library, the
TypeScript packages and a product at once. In a monorepo that is one commit and one CI run.

Bundles and module crates depend only on the facade `keel` and on `@globalart/keel`, so a
product's crates can move to their own repository later and be pulled into the `keel` build as
git dependencies on a published tag, while merging separate repositories back would be
expensive. The boundaries "kernel does not depend on host" and "modules see only the SDK" are
held by the cargo dependency graph and `keel-dev check`, not by repository boundaries. No
private cargo registry is needed for that. phoenix-ui and sqlmig stay in their own repositories
and are consumed as dependencies.

The cargo workspace has `members = ["crates/*", "bin/*", "products/*"]`; the frontend is a pnpm
workspace. The source leaves open whether product module crates sit under `crates/`
(`crates/dns-zones`) or nested inside the product directory (`products/dns/crates/dns-zones`); in
the latter case the workspace glob must be extended to `products/*/crates/*`. The table below
lists them with the product they belong to, not as a fixed path.

| Path | Contents |
|------|----------|
| `crates/types` | `keel-types`: descriptors, envelope, Principal/Account/Level, ListQuery/Page, events, `expr`, `check`; no I/O |
| `crates/sdk` | `keel-sdk`: `ModuleDef`, `Registrar`, `Call`, contributions, guards, `ListSource`, prelude |
| `crates/kernel` | `keel-kernel`: boot, config, module loader, dispatcher, metadata, access, storage, outbox, i18n, manifest, HTTP and `ctl` transports; kernel migrations |
| `crates/system` | `keel-system`: system modules, one cargo feature per module |
| `crates/host` | `keel-host`: `HostExecutor` client and the `host` module |
| `crates/host-agent` | `keel-host-agent`: the agent side, run as the `privileged` subcommand |
| `crates/release` | `keel-release`: the release manifest, its Ed25519 signature and the verifier the root side and the facade share, the bin `keel-release` (`keygen`, `sign`, `refresh`, `verify`), the public release keys under `keys/` and `channel.json` with the `contract` and `min_upgrade_from` of the next release; from M4 |
| `crates/storage` | `keel-storage`: the S3 backup target and the encryption of remote archives, not built yet; the installation backup and the site archives of `panel` are written to local directories without it |
| `crates/client` | `keel-client`: the Rust client of the wire contract, for a peer installation and the conformance suite: typed calls, credentials, on-behalf, keyed retries, a pinned certificate or the system roots, the pull feed as a stream; depends on `keel-types` alone; from M6 |
| `crates/testkit` | `keel-testkit` |
| `crates/conformance` | `keel-conformance` (bin) |
| `crates/dev` | `keel-dev` (bin): `types`, `descriptors dump \| diff \| check`, `views share`, `openapi` and, under `src/i18n/`, `i18n extract \| missing \| pull`; depends on `keel-types` alone |
| `crates/keel` | `keel`: the facade (lib): `Runtime` with the subcommands, `Running`, `Config`, `Change`, `ProductState`, re-exports of the SDK and the types; `embed_frontend!` from M2 |
| `bin/keel` | `keel-bin`: the binary `keel`; the explicit composition of every module and every bundle; the only crate that depends on product crates; cargo feature `demo` |
| `products/demo` | dogfood bundle without host access, a lib crate: `MANIFEST` and the `note` module; compiled in only with the `demo` feature |
| `products/dns` | bundle crate (lib): `MANIFEST`, `brand/`, `i18n/`, the product's section of `agent.yaml`; owns the module crates `dns-zones`, `dns-records`, `dns-templates` |
| `products/panel` | bundle crate (lib): `MANIFEST` with the editions `lite` and `pro` and the bridge to `dns`, `brand/`, `i18n/`; custom views in `frontend/products/panel`; owns the module crates `hosting-users`, `hosting-web`, `hosting-mail`, `hosting-db`, `hosting-ftp`, `hosting-ssl`, `hosting-backup`, `hosting-services`, `dns-local`, `dns-peer` |
| `frontend/` | pnpm workspace, Angular 19: `libs/keel` (entry points `meta`, `client`, `forms`, `renderer`, `shell`, `testing` and the nested `testing/playwright`), `apps/shell` (the one shell application), `products/<id>` (an Angular library per product with custom views; `demo` at M2), `.storybook/`, `e2e/` (the Playwright suite) and `tools/` (the generators, the theme boot script, the brotli twins, the version stamp) |
| `contracts/` | `fixtures/*.json`, `fixtures/descriptors/keel.*.json` (the kernel's descriptor snapshots); from M2 `schema/` (the JSON Schema of `keel-types`), `dump/` (the projected contract of the binary per level), `icons.json` (the phoenix icon set the kernel embeds) and the generated `openapi.json`, all committed for diffs; from M3 `i18n.json`, the translation projects with their dictionary directories and key prefixes; later `protocol.md`, the prose of the contract |
| `plugins/` | Python plugin SDK and example plugins |
| `deploy/` | `install.sh`, `uninstall.sh`, the four systemd units under `systemd/`, `config-sample.yaml` and `agent.yaml` with a section per first-party product; `README.md`, the operator's manual of the installer |
| `hack/` | `check-deps.sh`, `check-test-databases.sh` and `prove-binary.sh` from M0; `conformance.sh`, `check-docs.sh` and `check-install.sh` from M1; `descriptors.sh` and `e2e.sh` from M2; `check-release.sh`, `release.py` and `rehearsal.sh` from M4, with `agent.sh` (the in-process agent the scripts start) and the operator's proofs on a virtual machine: `install-vm.sh`, `update-vm.sh`, `backup-vm.sh`, `doctor-vm.sh`, `product-vm.sh`, `host-vm.sh` and `dns-vm.sh`; `check-size.sh`, `check-size-test.sh` and `size-cap` from M5, with `panel-vm.sh` and `bridge-vm.sh`; `peers-e2e.sh` from M6, two installations end to end in CI, with `peers-vm.sh`, the same on two machines |
| `docs/` | operator, module developer, plugin protocol, descriptor and migration documentation |
| `.github/workflows/` | `ci.yml`, `build.yml` (the musl artifact and the size check), `publish-npm.yml`, and from M4 `release.yml`, which publishes a `keel-v*` tag to the channel |

Crates are created lazily: about fourteen in total, seven at M0, which needs `keel-types`,
`keel-sdk`, `keel-kernel`, `keel-testkit`, the facade `keel`, `products/demo` and `bin/keel`, plus
the probe module `crates/sdk-probe` (`keel-sdk-probe`) that CI builds against `keel-sdk` alone
([05-modules.md](05-modules.md)); M1 adds `keel-system` and `keel-conformance`; M2 adds `keel-dev`
and, outside the workspace's dependency graph, the spike `keel-spike`, which M4 removed; M4 adds
`keel-release`, `keel-host`, `keel-host-agent` and the `dns` bundle with its module crates
`keel-dns-zones`, `keel-dns-records` and `keel-dns-templates`; M5 adds the `panel` bundle with its
module crates; M6 adds `keel-client`. A product without custom screens keeps no frontend code at
all; the one shell application renders its screens from descriptors.

The monorepo carries one known risk: the temptation to bypass the SDK "just for a minute". The
countermeasures are `keel-dev check` linting imports, bundles and module crates depending only on
the facade, `keel-conformance` running against the built binary, and a "dependency rehearsal"
(building `bin/keel` with one product's crates pulled from a git tag instead of the workspace)
from M4 onwards. After M7 a product's crates may live in their own repository and are pulled into
the `keel` build as dependencies pinned by this repository against the published `keel-v1.0.0`;
the binary is always built here. The alternatives considered for this decision are recorded in
[11-decisions.md](11-decisions.md).

## Versioning

There are three versioning planes, each with its own rule.

| Plane | What is versioned | Rule |
|-------|-------------------|------|
| Compile-time | `keel-sdk` | semver; the kernel and the SDK are released under one version; `cargo-semver-checks` runs on the facade from the first commit; the public `keel-sdk` is frozen at 1.0, `keel_sdk::system` is semver-exempt until then |
| Wire | `PROTOCOL` | reported in `GET /api/v1/instance` and in the plugin manifest; versions N and N-1 are supported |
| Data | descriptor `"keel": 1`, `/api/v1` | a breaking change ships as `/api/v2` alongside `/api/v1`; the major version of `@globalart/keel` equals the descriptor version |

There is one release train. Tags follow `keel-vX.Y.Z` for the binary, the kernel, the SDK and the
npm package; the `keel` binary has one version, and that version is the version of every product
it carries. Product ids carry no semver of their own and there are no `<product>-vX.Y.Z` tags.
Every first-party crate takes the workspace version, so `ModuleDef.version` and the module
ranges a plugin manifest names ([10-contracts.md](10-contracts.md)) are expressed against the
same number; a module crate that lives in its own repository keeps a cargo version for dependency
pinning, which is not a product version. The wire contract itself (envelope, descriptor, list
contract, golden fixtures) is specified in [10-contracts.md](10-contracts.md).
