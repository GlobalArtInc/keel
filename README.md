# keel

The framework under a line of control panels. A product of the line — KeelDNS, KeelPanel, later
KeelBill, KeelVM, KeelIP — is a set of modules composed on this core; every product ships in the
one static binary `keel` and is enabled per installation. The core itself knows nothing about
zones, sites or tariffs: it knows how to run a named function, who is calling it, how to describe
its screen to the frontend, and how products discover and trust each other.

Backend in Rust; frontend in Angular on `@globalart/phoenix-ui`; Postgres on the host, one
database per host; one binary `keel` per host with the frontend embedded.

## Status

The design is in `docs/design/`, the order of work in `docs/plan.md`. M0 and M1 are built; the
milestones after them are designed, not yet built.

| Milestone | Scope | State |
|---|---|---|
| M0 | Walking skeleton: types, SDK, kernel, dispatcher, storage, control socket, `demo` bundle | done: the binary `keel` serves the `keel` module and the `demo` bundle over HTTP and the control socket, with every check of the M0 exit checklist in `docs/plan.md` green |
| M1 | Identity, access and audit: `auth`, `settings`, `audit`, conformance skeleton | done: the binary composes `auth`, `settings` and `audit`; login, tokens, TOTP, `su`, login keys, IP rules, rate limit, lockout, settings, preferences and the audit trail are served over HTTP and the control socket, `keel-conformance` is green against the built binary, and every check of the M1 exit checklist in `docs/plan.md` is green |
| M2 | Frontend runtime `@globalart/keel` and the host agent spike | |
| M3 | Background work and feedback: `tasks`, `cron`, `notify`, `webhooks`, SSE, TLS | |
| M4 | Operator plane, `keel-host`, KeelDNS alpha | |
| M5 | KeelPanel, `license`, `stats`, report and dashboard renderers | |
| M6 | Products working together: `peers`, events, services, `keel-client` | |
| M7 | External plugins, SDK 1.0, product module crates may move to their own repositories | |
| M8 | Cluster of nodes | |

## Layout

```
crates/      keel-types, keel-sdk, keel-kernel, keel-system, keel-testkit, keel-conformance,
             keel-host, ... (from M0)
products/    demo, dns (KeelDNS), panel (KeelPanel) — one bundle (lib crate) each
bin/keel/    keel-bin — the binary keel: the composition of every module and every bundle of
             the workspace; the only crate that depends on product crates. Its cargo feature
             demo is on by default so the workspace, CI and hack/prove-binary.sh carry the
             demo bundle; release builds pass --no-default-features and carry no demo
frontend/    pnpm workspace: @globalart/keel, the shell application and the product view
             libraries (from M2)
contracts/   golden fixtures and the kernel's descriptor snapshots; protocol.md and the
             generated openapi.json come later (from M2)
deploy/      install.sh, uninstall.sh, the systemd unit and the sample config, with a README
             of what the installer does today and what waits for M4
docs/        design and plan
hack/        scripts CI runs
```

## Building and running

The checks below are the ones CI runs, in the same order; run them before pushing. The tests,
`hack/prove-binary.sh` and `hack/conformance.sh` use the Postgres superuser connection in
`KEEL_TEST_DATABASE_URL`, from which every test creates and drops a database of its own; when
the variable is unset, `keel-testkit` starts the container `keel-testkit-postgres` from
`postgres:16` through the local Docker socket, keeps it running for the next test process and
prints the URL it uses, which is the path the `env -u` command below proves:

```
export KEEL_TEST_DATABASE_URL=postgres://keel:keel@127.0.0.1:5432/postgres
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
env -u KEEL_TEST_DATABASE_URL cargo test -p keel-testkit
cargo test -p keel-bin --no-default-features
cargo build -p keel-bin
hack/prove-binary.sh target/debug/keel
hack/check-install.sh target/debug/keel
hack/conformance.sh target/debug/keel
hack/check-test-databases.sh
hack/check-deps.sh
hack/check-docs.sh
```

`hack/check-install.sh` checks the installer under `deploy/`: `bash -n` and shellcheck over
the scripts, `--help` printing the usage, every subcommand and flag they call found in the
built binary's `--help`, every function they call defined in the workspace, and the sample
config parsing. `hack/check-docs.sh` holds the documents to the code: the names M1 introduced are found under
both `docs/design/` and `crates/`, every test and command the M1 exit checklist of `docs/plan.md`
cites exists, and the list above, the "How we work" section of the plan and `ci.yml` name the same
commands. CI ends with `cargo-semver-checks` against the facade crate `keel`, and a second job
runs the `keel-system` matrix, `cargo clippy` and `cargo test -p keel-system
--no-default-features --features <set>` for each module alone (`auth`, `settings`, `audit`) and
for the crate without each in turn (`settings,audit`, `auth,audit`, `auth,settings`); a test of
the crate declares the modules it needs as `required-features` and is skipped when they are off.

`cargo build -p keel-bin` produces `target/debug/keel` with the `demo` bundle compiled in, since
the feature `demo` of `keel-bin` is on by default; `--no-default-features` builds the binary a
release ships, without it. `hack/prove-binary.sh` drives the built binary through a fresh
database: it enables `demo`, starts `serve`, exercises the socket and HTTP, then repeats the run
with `demo` disabled. The same steps by hand, against the default database `keel` when no
`config.yaml` names another one:

```
keel product enable demo          # writes the enabled set; the service must be stopped
keel serve                        # migrates, then listens on 127.0.0.1:1500 and the socket
keel ctl note.edit title=x        # the envelope of the call, as JSON
keel ctl note.list --out table    # the rows rendered by ui.columns with translated headers
keel product list                 # every bundle the binary carries, enabled or disabled
```

Installing the built binary on a host is `deploy/install.sh --product demo --binary <artifact>`
on a clean Debian or Ubuntu; `deploy/README.md` describes the steps, the flags and what the
installer leaves to later milestones.

`hack/conformance.sh target/debug/keel` prepares a fresh database and `data_dir` the same way,
enables `demo`, creates an admin and a user in a second account over `ctl`, starts `serve` and
runs `keel-conformance`, the contract suite: a black-box client of the started binary, typed
with `keel-types` alone, that prints one line per check and exits non-zero on any failure. It
takes `--url`, `--admin-login`, `--admin-password` (or `KEEL_CONFORMANCE_ADMIN_PASSWORD`) and
`--user-login`, the login of a user in another account.

`keel --config <path>` (or `KEEL_CONFIG`) points at another configuration file; `keel ctl
--socket <path>` at another control socket than `/run/keel/core.sock`. The descriptor snapshots
of the kernel under `contracts/fixtures/descriptors/` are refreshed with
`KEEL_UPDATE_SNAPSHOTS=1 cargo test -p keel-kernel` after reviewing the diff.

## Documents

- `docs/design/01-overview.md` — what the core is, layers, repository and versioning
- `docs/design/02-functions.md` — functions, descriptors, the dispatcher pipeline
- `docs/design/03-access.md` — accounts, principals, levels, scoping, limits
- `docs/design/04-storage.md` — Postgres, migrations, identifiers, secrets
- `docs/design/05-modules.md` — modules, the SDK, system modules, tooling
- `docs/design/06-host.md` — the root boundary and the host agent
- `docs/design/07-ecosystem.md` — peers, events, services, shared identity
- `docs/design/08-frontend.md` — the frontend library and its boundary with phoenix-ui
- `docs/design/09-operations.md` — delivery, systemd, updates, backups, support
- `docs/design/10-contracts.md` — wire contracts with examples
- `docs/design/11-decisions.md` — decisions, alternatives, risks, open questions
- `docs/plan.md` — milestones, the M0 and M1 work breakdowns and their exit checklists
