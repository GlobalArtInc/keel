# keel

The framework under a line of control panels. A product of the line — KeelDNS, KeelPanel, later
KeelBill, KeelVM, KeelIP — is a set of modules composed on this core; every product ships in the
one static binary `keel` and is enabled per installation. The core itself knows nothing about
zones, sites or tariffs: it knows how to run a named function, who is calling it, how to describe
its screen to the frontend, and how products discover and trust each other.

Backend in Rust; frontend in Angular on `@globalart/phoenix-ui`; Postgres on the host, one
database per host; one binary `keel` per host with the frontend embedded.

## Status

The design is in `docs/design/`, the order of work in `docs/plan.md`. M0, M1, M2 and M3 are
built; the milestones after them are designed, not yet built.

| Milestone | Scope | State |
|---|---|---|
| M0 | Walking skeleton: types, SDK, kernel, dispatcher, storage, control socket, `demo` bundle | done: the binary `keel` serves the `keel` module and the `demo` bundle over HTTP and the control socket, with every check of the M0 exit checklist in `docs/plan.md` green |
| M1 | Identity, access and audit: `auth`, `settings`, `audit`, conformance skeleton | done: the binary composes `auth`, `settings` and `audit`; login, tokens, TOTP, `su`, login keys, IP rules, rate limit, lockout, settings, preferences and the audit trail are served over HTTP and the control socket, `keel-conformance` is green against the built binary, and every check of the M1 exit checklist in `docs/plan.md` is green |
| M2 | Frontend runtime `@globalart/keel` and the host agent spike | done: the binary embeds the shell and serves it beside the API; `@globalart/keel` with its six entry points and the nested `testing/playwright` renders the `demo` bundle's list, form, wizard and custom view from descriptors alone and switches `en` and `ru`; `publish-npm.yml` publishes it to Nexus from the closing commit; the host spike `bin/spike` created an OS user and a vhost through its root agent and rolled an invalid template back on `keel-demo`; the musl binary with the frontend is 19.8 MB against the 40 MB cap; every CI-run item of the M2 exit checklist in `docs/plan.md` is green, and the three operator-run items are ticked by hand on `keel-demo` after the push |
| M3 | Background work and feedback: `tasks`, `cron`, `notify`, `webhooks`, SSE, TLS | done: the binary composes `tasks`, `cron`, `notify` and `webhooks`; `note.export` runs as a task with progress, log, cancellation and a checkpoint that survives a restart, watched in the browser through the progress modal and the task drawer; a cron job raises a problem that reaches the dashboard, the notification centre and the mail catcher; the event stream at `/api/v1/events` carries `data.invalidate` scoped to the principal's account; `keel.export`, group editing through `ui.multi`, the `upload` kind with the brand logo as its first upload, TLS with a generated certificate and `trusted_proxies` are served; `@globalart/keel` pins phoenix-ui 0.3.1 and `keel-dev i18n missing` holds `en` and `ru` complete; every CI-run item of the M3 exit checklist in `docs/plan.md` is green, and the operator-run items are ticked by hand on `keel-demo` after the push; `keel.account.delete` as a task and the per-session cap of the stream were left undone and are named in the checklist |
| M4 | Operator plane, `keel-host`, KeelDNS alpha | |
| M5 | KeelPanel, `license`, `stats`, report and dashboard renderers | |
| M6 | Products working together: `peers`, events, services, `keel-client` | |
| M7 | External plugins, SDK 1.0, product module crates may move to their own repositories | |
| M8 | Cluster of nodes | |

## Layout

```
crates/      keel-types, keel-sdk, keel-kernel, keel-system, keel-testkit, keel-conformance,
             keel (the facade), keel-dev (crates/dev: types, descriptors, views share, openapi,
             and crates/dev/src/i18n with extract, missing and pull), keel-host, ... (from M0)
products/    demo, dns (KeelDNS), panel (KeelPanel) — one bundle (lib crate) each
bin/keel/    keel-bin — the binary keel: the composition of every module and every bundle of
             the workspace; the only crate that depends on product crates. Its cargo feature
             demo is on by default so the workspace, CI and hack/prove-binary.sh carry the
             demo bundle; release builds pass --no-default-features and carry no demo
bin/spike/   keel-spike — the host spike of M2: a root agent admitting one uid by peer
             credentials with host.user.create and host.template.apply under a policy, its
             client, unit and policy, and hack/spike-vm.sh that proves it on a machine; depends
             on no workspace crate and is replaced by keel-host-agent in M4
frontend/    pnpm workspace (node 24, pnpm 11, Angular 19): libs/keel is @globalart/keel with
             the entry points meta, client, forms, renderer, shell and testing and the nested
             testing/playwright; apps/shell the one shell application the binary embeds;
             products/demo the demo's view library; .storybook, e2e (Playwright) and tools (the
             generators, theme boot, brotli, stamp)
contracts/   golden fixtures, the kernel's descriptor snapshots, the JSON Schema of keel-types
             (schema/), the projected contract of the binary per level (dump/), the phoenix
             icon set (icons.json), the generated openapi.json and the translation projects
             (i18n.json); protocol.md comes later
deploy/      install.sh, uninstall.sh, the systemd unit and the sample config, with a README
             of what the installer does today and what waits for M4
docs/        design and plan
hack/        scripts CI runs
```

## Building and running

The checks below are the ones CI runs, in the same order; run them before pushing. The job
`check` builds the shell first, since `cargo build -p keel-bin` embeds `frontend/dist/shell`
through `embed_frontend!`; the tests, `hack/prove-binary.sh`, `hack/conformance.sh`,
`hack/descriptors.sh` and `hack/e2e.sh` use the Postgres superuser connection in
`KEEL_TEST_DATABASE_URL`, from which every test creates and drops a database of its own; when
the variable is unset, `keel-testkit` starts the container `keel-testkit-postgres` from
`postgres:16` through the local Docker socket, keeps it running for the next test process and
prints the URL it uses, which is the path the `env -u` command below proves. The frontend
commands run in `frontend/` with `NEXUS_NPM_TOKEN` in the environment, the token of the private
registry `@globalart/phoenix-ui` comes from; `pnpm exec playwright install chromium`
is needed once for the browser suite:

```
export KEEL_TEST_DATABASE_URL=postgres://keel:keel@127.0.0.1:5432/postgres
cargo fmt --all --check
pnpm install --frozen-lockfile
pnpm build
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
env -u KEEL_TEST_DATABASE_URL cargo test -p keel-testkit
cargo run -p keel-dev -- types --out contracts/schema --check
cargo test -p keel-bin --no-default-features
cargo build -p keel-bin
hack/prove-binary.sh target/debug/keel
hack/check-install.sh target/debug/keel
hack/conformance.sh target/debug/keel
hack/descriptors.sh target/debug/keel
cargo run -p keel-dev -- i18n missing --dump contracts/dump
pnpm exec playwright install chromium
hack/e2e.sh target/debug/keel
hack/check-test-databases.sh
hack/check-deps.sh
hack/check-docs.sh
```

The job `frontend` runs the workspace's own checks in `frontend/`, the last command in
`frontend/dist/keel` after the version stamp:

```
pnpm install --frozen-lockfile
pnpm generate
git diff --exit-code
pnpm lint
pnpm format
pnpm test
pnpm build
pnpm build-storybook
pnpm exec http-server storybook-static --port 6006 --silent &
pnpm exec wait-on http://127.0.0.1:6006/index.json
pnpm test-storybook
node tools/stamp.mjs
pnpm publish --dry-run --no-git-checks
```

`pnpm generate` writes what the repository commits and `git diff --exit-code` holds current: the
TypeScript wire types from `contracts/schema/`, `contracts/icons.json` from the phoenix icon set and
the `KEEL_BOOT` fixtures of `@globalart/keel/testing`. `hack/descriptors.sh` starts the built binary
on a fresh database, checks `contracts/dump/` against its answers per level, checks
`contracts/openapi.json` against the dump and prints the custom-views share per product and for the
composition, failing above 20 %; `KEEL_UPDATE_DUMP=1` rewrites the dump and the document after a
reviewed change. `cargo run -p keel-dev -- i18n missing --dump contracts/dump` derives every key the
descriptors of the dump emit and fails on one the committed dictionaries do not translate in `en` or
`ru`. `hack/e2e.sh` prepares a site the same way, with an admin, a reseller and a user, an email
channel to a mail catcher (`KEEL_TEST_SMTP_URL` and `KEEL_TEST_SMTP_API`, or a container the script
starts through podman or docker) and an overdue todo whose problem the poll job raises, and runs the
Playwright suite of `frontend/e2e/` against it; the traces of a failed run are kept as a CI
artifact. Every script that starts the binary speaks https to it with the certificate the binary
generates under the site's data directory. `hack/check-install.sh` checks the installer under
`deploy/`: `bash -n` and shellcheck over the scripts, `--help` printing the usage, every subcommand
and flag they call found in the built binary's `--help`, every function they call defined in the
workspace, and the sample config parsing. `hack/check-docs.sh` holds the documents to the code: the
names M1, M2 and M3 introduced are found under `docs/design/` and under `crates/` or
`frontend/libs/keel`, every Rust test, Jest and Playwright spec and command the M1, M2 and M3 exit
checklists of `docs/plan.md` cite exists, and the lists above, the "How we work" section of the plan
and `ci.yml` name the same commands. CI ends with `cargo-semver-checks` against the facade crate
`keel`, and a second job runs the `keel-system` matrix, `cargo clippy` and `cargo test -p
keel-system --no-default-features --features <set>` over fourteen sets: each of the seven modules
alone (`auth`, `settings`, `audit`, `tasks`, `cron`, `notify`, `webhooks`), the crate without each
of `auth`, `settings`, `audit` and `tasks` in turn, and the nested prefixes
`auth,settings,audit,tasks`, `…,cron` and `…,cron,notify`, against Postgres and the mail catcher;
every integration test of the crate is a module of the one test binary `tests/system/main.rs`, gated
with `#[cfg(feature = ...)]` on the modules it needs, so a test is skipped when they are off. The
Build workflow builds the musl release of the binary with the frontend, fails when it reaches 40 MB
and uploads it with the host spike as artifacts, after `keel-dev i18n pull` has refreshed the
dictionaries when the translation token is present; `publish-npm.yml` publishes `@globalart/keel` to
Nexus on a `keel-v*` tag or by hand, stamped with the binary's version.

### Running by hand

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

`keel serve` also serves the panel: `https://localhost:1500/` opens the embedded shell, whose
login is `auth.login` over the same API. The listener speaks TLS alone, with the self-signed
certificate the first start writes to `<data_dir>/tls/cert.pem` and logs by its SHA-256
fingerprint, or with the pair `tls.cert` and `tls.key` of `config.yaml` name; a browser warns
once about the self-signed certificate and the fingerprint tells the operator it is the right
one. Behind a reverse proxy, `trusted_proxies` lists the proxy addresses whose `X-Forwarded-For`
and `X-Forwarded-Host` the kernel believes.

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
- `docs/plan.md` — milestones, the M0, M1, M2 and M3 work breakdowns and their exit checklists
